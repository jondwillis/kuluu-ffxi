#!/usr/bin/env python3
import argparse
import base64
import fcntl
import hashlib
import ipaddress
import json
import math
import os
from pathlib import Path
import platform
import re
import shutil
import stat
import struct
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import zipfile

SCRIPT_DIR = Path(__file__).resolve().parent
AHK_VERSION = '2.0.30'
AHK_ZIP_SHA256 = '1e0ece7ecb2d5646e55758150472677996a57f9db5c83d2996d8b47b9e2f31a4'
AHK_EXE_SHA256 = '2eab99848ebc07f8ab0cc448bf69af8c700b2fc7fcd1ff8794df743dcc58da51'
AHK_URL = f'https://github.com/AutoHotkey/AutoHotkey/releases/download/v{AHK_VERSION}/AutoHotkey_{AHK_VERSION}.zip'
JEV_URL = 'https://api.typesafe.ai/v1/systemone'
JEV_MODEL = 'jev-1.14.0'
MIN_IMAGE_EDGE = 280
MAX_IMAGE_EDGE = 1568
MAX_DOWNLOAD_BYTES = 20 * 1024 * 1024
PROFILE_VERSION = 1
COMMAND_TIMEOUT_SECONDS = 30
MAX_KEY_HOLD_SECONDS = 5
STARTUP_WINDOW_TIMEOUT_SECONDS = 30
STARTUP_WINDOW_POLL_SECONDS = 0.25
STARTUP_ENTER_DELAY_SECONDS = 5
STARTUP_PORT_OPTIONS = frozenset(('--authport', '--dataport', '--viewport', '--profileport'))
STARTUP_FLAG_OPTIONS = frozenset(('--hairpin', '--hide'))
STARTUP_LANGUAGES = frozenset(('JP', 'US', 'EU', '0', '1', '2'))
MAX_TCP_PORT = 65535
MAX_WHEEL_COUNT = 20
MAX_TEXT_BYTES = 4096
MAX_DRAG_PIXELS = 512
MILLISECONDS_PER_SECOND = 1000
MAX_DIALOGUE_ACTIONS = 40
MAX_DIALOGUE_SECONDS = 300
MAX_UNCHANGED_DIALOGUE_OUTCOMES = 2
DIALOGUE_TEXT_TOP_FRACTION = 0.75
DIALOGUE_TEXT_RIGHT_FRACTION = 0.8
DIALOGUE_WAIT_SECONDS = 1
MAX_INPUT_CAPTURE_AGE_SECONDS = 60
MAX_LOW_CONFIDENCE_RETRIES = 2
DECISION_CONFIDENCE_THRESHOLD = 0.9
DEFAULT_HOLD_SECONDS = '0.2'
WIN32_KEY_COLUMN = 3
PNG_SIGNATURE = b'\x89PNG\r\n\x1a\n'
PNG_SIZE_OFFSET = 16
PNG_HEADER_BYTES = 24
PRIVATE_PERMISSION_MASK = 0o077


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def run(args, **kwargs):
    return subprocess.run([str(arg) for arg in args], check=True, timeout=COMMAND_TIMEOUT_SECONDS, **kwargs)


def private_write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd, temporary = tempfile.mkstemp(dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as stream:
            stream.write(data)
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def profile_path(name):
    if not re.fullmatch(r'[A-Za-z0-9_-]+', name):
        raise ValueError('Profile name must contain only letters, numbers, underscores, or hyphens.')
    root = Path(os.environ.get('XDG_CONFIG_HOME', Path.home() / '.config'))
    return root / 'ffxi-observe' / f'{name}.macos.json'


def checked_file(value, executable=False):
    path = Path(value).expanduser().resolve()
    if not path.is_file() or (executable and not os.access(path, os.X_OK)):
        raise ValueError(f'Required file is unavailable: {path}')
    return path


def install_ahk(archive=None):
    root = Path(os.environ.get('XDG_CACHE_HOME', Path.home() / '.cache'))
    target = root / 'ffxi-observe' / f'AutoHotkey-{AHK_VERSION}' / 'AutoHotkey64.exe'
    if target.is_file() and digest(target) == AHK_EXE_SHA256:
        return target
    if archive:
        data = checked_file(archive).read_bytes()
    else:
        with urllib.request.urlopen(AHK_URL, timeout=COMMAND_TIMEOUT_SECONDS) as response:
            data = response.read(MAX_DOWNLOAD_BYTES + 1)
    if len(data) > MAX_DOWNLOAD_BYTES or hashlib.sha256(data).hexdigest() != AHK_ZIP_SHA256:
        raise ValueError('AutoHotkey archive checksum failed. No executable was installed.')
    with tempfile.TemporaryFile() as stream:
        stream.write(data)
        stream.seek(0)
        with zipfile.ZipFile(stream) as archive_file:
            executable = archive_file.read('AutoHotkey64.exe')
    if hashlib.sha256(executable).hexdigest() != AHK_EXE_SHA256:
        raise ValueError('AutoHotkey executable checksum failed.')
    private_write(target, executable)
    return target


def setup(args, path):
    if path.exists() and not args.replace:
        raise ValueError('Profile already exists. Use --replace to update it.')
    startup = startup_profile(args.server, getattr(args, 'startup_enter', False), args.loader_arg)
    runner = checked_file(shutil.which(args.runner) or args.runner, executable=True)
    prefix = Path(args.prefix).expanduser().resolve()
    checked_file(prefix / 'system.reg')
    install = Path(args.install).expanduser().resolve()
    dll = checked_file(install / 'FFXiMain.dll')
    loader = checked_file(args.loader)
    loader_hash = digest(loader)
    cache = Path(os.environ.get('XDG_CACHE_HOME', Path.home() / '.cache'))
    cached_loader = cache / 'ffxi-observe' / 'loaders' / loader_hash / loader.name
    private_write(cached_loader, loader.read_bytes())
    loader = cached_loader
    if any(re.search(r'(?i)(--?(?:user|pass|password|username)(?:=|\b))', arg) for arg in args.loader_arg):
        raise ValueError('Do not store login credentials in loader arguments. Use FFXI_USER and FFXI_PASS.')
    ahk = install_ahk(args.ahk_archive)
    version = run([runner, '--version'], capture_output=True, text=True).stdout.strip()
    config = dict(schema=PROFILE_VERSION, runner=str(runner), runner_sha256=digest(runner),
                  runner_version=version, prefix=str(prefix), install=str(install),
                  client_sha256=digest(dll), loader=str(loader), loader_sha256=digest(loader),
                  loader_args=args.loader_arg, server=args.server, startup_enter=startup, window_title='FINAL FANTASY XI',
                  ahk=str(ahk), ahk_sha256=AHK_EXE_SHA256, jev_model=JEV_MODEL,
                  jev_key_file=str(Path(args.jev_key_file).expanduser().resolve()),
                  machine=platform.machine(), macos=platform.mac_ver()[0])
    private_write(path, (json.dumps(config, indent=2) + '\n').encode())
    print(f'Profile saved: {path}')
    print(json.dumps(dict(startup_enter=config['startup_enter'])))


def loopback_server(server):
    if server == 'localhost':
        return True
    try:
        return ipaddress.ip_address(server).is_loopback
    except ValueError:
        return False


def validate_startup_loader_args(arguments):
    options = {}
    index = 0
    while index < len(arguments):
        option = arguments[index]
        if option in options:
            raise ValueError('Duplicate startup loader options are ambiguous. No startup input was sent.')
        if option in STARTUP_FLAG_OPTIONS:
            options[option] = True
            index += 1
            continue
        if option not in STARTUP_PORT_OPTIONS and option != '--lang':
            raise ValueError('Unsupported startup loader option. Server and JSON destination overrides are not permitted.')
        if index + 1 >= len(arguments):
            raise ValueError('A startup loader option has no value.')
        value = arguments[index + 1]
        if option in STARTUP_PORT_OPTIONS:
            if not isinstance(value, str) or not re.fullmatch(r'[0-9]+', value) or not 1 <= int(value) <= MAX_TCP_PORT:
                raise ValueError('Startup loader ports must be decimal integers from 1 to 65535.')
            options[option] = int(value)
        elif value not in STARTUP_LANGUAGES:
            raise ValueError('Startup loader language must be JP, US, EU, 0, 1, or 2.')
        else:
            options[option] = value
        index += 2
    return options


def startup_profile(server, enabled, loader_args=()):
    if enabled and not loopback_server(server):
        raise ValueError('Startup Enter requires an explicit local private-server profile with a loopback server.')
    options = validate_startup_loader_args(loader_args) if enabled else {}
    return dict(loader_options=options, enabled=enabled, local_private_server=enabled, action='enter', caller_policy='permitted' if enabled else 'caller',
                delay_seconds=STARTUP_ENTER_DELAY_SECONDS, timeout_seconds=STARTUP_WINDOW_TIMEOUT_SECONDS)


def submit_startup_enter(config, process):
    deadline = time.monotonic() + STARTUP_WINDOW_TIMEOUT_SECONDS
    target = None
    while time.monotonic() < deadline:
        if process.poll() not in (None, 0):
            raise ValueError('The startup loader failed. No startup input was sent.')
        target = window(config, allow_missing=True)
        if target is not None:
            break
        time.sleep(STARTUP_WINDOW_POLL_SECONDS)
    if target is None:
        raise ValueError('The startup window wait expired. No startup input was sent.')
    time.sleep(STARTUP_ENTER_DELAY_SECONDS)
    if time.monotonic() >= deadline:
        raise ValueError('The startup window wait expired. No startup input was sent.')
    current = window(config)
    if current != target:
        raise ValueError('The startup window changed. No startup input was sent.')
    key(config, 'enter', DEFAULT_HOLD_SECONDS)
    return dict(status='submitted', action='enter', count=1, window=target, visible_result_verified=False)


def launch(config, args):
    unlocked()
    user, password = os.environ.get('FFXI_USER'), os.environ.get('FFXI_PASS')
    if not user or not password:
        raise ValueError('Set FFXI_USER and FFXI_PASS before launch.')
    arguments = [config['runner'], config['loader'], *config['loader_args'],
                 '--server', config['server'], '--user', user, '--pass', password]
    env = environment(config)
    env['WINEDLLOVERRIDES'] = 'mscoree,mshtml='
    profile = config.get('startup_enter', {})
    if not profile.get('enabled'):
        os.chdir(Path(config['loader']).parent)
        os.execve(config['runner'], arguments, env)
        return
    if profile.get('action') != 'enter' or not profile.get('local_private_server') or not loopback_server(config['server']):
        raise ValueError('Startup Enter is disabled for this profile. Configure an explicit local private-server profile first.')
    startup_profile(config['server'], True, config['loader_args'])
    policy = caller_policy(dict(screen='agreement'), getattr(args, 'agreement_policy', None) or profile.get('caller_policy', 'caller'))
    output = dict(operation='launch', startup_enter_requested=True, startup_action='enter', caller_policy=policy)
    if policy['status'] != 'permitted':
        output.update(status=policy['status'], submitted=False, startup_enter_skipped=True)
        print(json.dumps(output), flush=True)
        os.chdir(Path(config['loader']).parent)
        os.execve(config['runner'], arguments, env)
        return output
    if config.get('offscreen_control'):
        raise ValueError('Startup Enter requires a visible window. Offscreen startup input is disabled.')
    if window(config, allow_missing=True) is not None:
        raise ValueError('A game window already exists. No startup input or launch was attempted.')
    output.update(status='starting', submitted=False)
    print(json.dumps(output), flush=True)
    process = subprocess.Popen(arguments, cwd=Path(config['loader']).parent, env=env)
    output.update(submit_startup_enter(config, process), launched=True, loader_pid=process.pid)
    print(json.dumps(output), flush=True)
    return output


def load(path):
    config = json.loads(path.read_text())
    if config['schema'] != PROFILE_VERSION:
        raise ValueError('Unsupported profile version. Run setup again.')
    for field in ('runner', 'loader', 'ahk'):
        checked_file(config[field], executable=field == 'runner')
        if digest(config[field]) != config[field + '_sha256']:
            raise ValueError(f'{field} changed. Verify the update, then run setup with --replace.')
    checked_file(Path(config['prefix']) / 'system.reg')
    if digest(checked_file(Path(config['install']) / 'FFXiMain.dll')) != config['client_sha256']:
        raise ValueError('The client changed. Verify the update, then run setup with --replace.')
    return config


def environment(config):
    env = os.environ.copy()
    for name in list(env):
        if name.startswith('FFXI_OBSERVE_') or name in ('HXI_BG', 'HXI_GAME_RE', 'HXI_OWNER_RE'):
            del env[name]
    env.update(WINEPREFIX=config['prefix'], WINEDEBUG='-all', FFXI_OBSERVE_HOST='macos',
               FFXI_OBSERVE_RUNNER=config['runner'], FFXI_OBSERVE_AHK=config['ahk'],
               FFXI_OBSERVE_WINDOW_TITLE=config['window_title'])
    return env


def permissions():
    script = '''ObjC.import('CoreGraphics');
ObjC.bindFunction('CGPreflightScreenCaptureAccess', ['bool', []]);
JSON.stringify({capture: $.CGPreflightScreenCaptureAccess()});'''
    result = run(['osascript', '-l', 'JavaScript', '-e', script], capture_output=True, text=True)
    if not json.loads(result.stdout)['capture']:
        raise ValueError('Grant Screen Recording to the calling application, then restart it.')


def unlocked():
    result = run(['ioreg', '-l', '-w0'], capture_output=True, text=True)
    if '"CGSSessionScreenIsLocked"=Yes' in result.stdout:
        raise ValueError('Unlock the Mac before sending input.')


def window(config, allow_missing=False, include_offscreen=False):
    env = environment(config)
    env['SCRIPT_DIR'] = str(SCRIPT_DIR)
    env['WINDOW_INCLUDE_OFFSCREEN'] = '1' if include_offscreen else '0'
    command = '. "$SCRIPT_DIR/lib/common.sh"; . "$SCRIPT_DIR/lib/macos.sh"; WINDOW_OWNER=.; host_windows_json'
    result = run(['bash', '-c', command], env=env, capture_output=True, text=True)
    matches = [item for item in json.loads(result.stdout)
               if config['window_title'].lower() in item['name'].lower()]
    if not matches and allow_missing:
        return None
    if len(matches) != 1:
        raise ValueError(f'Expected one FFXI window; found {len(matches)}. Close duplicate clients or start the game.')
    return matches[0]


def png_size(path):
    data = path.read_bytes()
    if len(data) < PNG_HEADER_BYTES or not data.startswith(PNG_SIGNATURE):
        raise ValueError('Expected a PNG capture.')
    return struct.unpack('>II', data[PNG_SIZE_OFFSET:PNG_HEADER_BYTES])


def capture(config, path):
    permissions()
    path = path.expanduser().resolve()
    run(['caffeinate', '-u', '-t', '2'], capture_output=True)
    target = window(config, allow_missing=True)
    if target is None:
        target = window(config, include_offscreen=True)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.unlink(missing_ok=True)
    run(['screencapture', '-x', '-o', '-l', target['id'], path], capture_output=True)
    width, height = png_size(path)
    if not width or not height:
        raise ValueError('The capture has invalid dimensions.')
    metadata = dict(image=str(path), image_sha256=digest(path), captured_unix=time.time(),
                    width=width, height=height, scale=width / target['w'], window=target)
    private_write(path.with_suffix('.capture.json'), (json.dumps(metadata, indent=2) + '\n').encode())
    print(json.dumps(metadata))
    return path


def key(config, name, seconds):
    duration = float(seconds)
    if not math.isfinite(duration) or not 0 <= duration <= MAX_KEY_HOLD_SECONDS:
        raise ValueError(f'Hold duration must be between 0 and {MAX_KEY_HOLD_SECONDS} seconds.')
    keys = dict(line.split('\t')[:1] + [line.split('\t')[WIN32_KEY_COLUMN]]
                for line in (SCRIPT_DIR / 'lib/keys.tsv').read_text().splitlines()
                if line and not line.startswith('#'))
    if name not in keys:
        raise ValueError('Unknown logical key. Use observe.sh keys to list the names.')
    wine_input(config, [keys[name], round(duration * MILLISECONDS_PER_SECOND)])
    print(json.dumps(dict(key=name, hold_seconds=duration, submitted=True,
                          visible_result_verified=False)))


def wine_input(config, arguments, text_bytes=None, allow_missing_window=False):
    unlocked()
    if allow_missing_window:
        window(config, allow_missing=True)
    elif config.get('offscreen_control'):
        validate_input_capture(config)
    else:
        window(config)
    lock_id = hashlib.sha256(config['prefix'].encode()).hexdigest()
    lock = profile_path('input-' + lock_id).with_suffix('.lock')
    lock.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with lock.open('a') as stream:
        try:
            fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise ValueError('Another packaged input command is active for this Wine prefix.') from None
        env = environment(config)
        helper = SCRIPT_DIR / 'lib/wine-key.ahk'
        converted = run([config['runner'], 'winepath.exe', '-w', helper],
                        env=env, capture_output=True, text=True).stdout.strip()
        with tempfile.TemporaryDirectory(prefix='ffxi-observe-input-') as temporary:
            if text_bytes is not None:
                text_path = Path(temporary) / 'input.txt'
                private_write(text_path, text_bytes)
                guest_path = run([config['runner'], 'winepath.exe', '-w', text_path],
                                 env=env, capture_output=True, text=True).stdout.strip()
                arguments = ['text', guest_path, 'literal']
            run([config['runner'], config['ahk'], '/ErrorStdOut', converted,
                 config['window_title'], *arguments], env=env, capture_output=True)


def validate_input_capture(config):
    if not config.get('input_frame'):
        raise ValueError('Offscreen input requires a fresh packaged capture.')
    frame = Path(config['input_frame']).expanduser().resolve()
    metadata = json.loads(frame.with_suffix('.capture.json').read_text())
    age = time.time() - metadata['captured_unix']
    if not 0 <= age <= MAX_INPUT_CAPTURE_AGE_SECONDS or metadata['image_sha256'] != digest(frame):
        raise ValueError('The input capture is stale or changed. Capture the game again.')
    target = window(config, include_offscreen=True)
    if any(metadata['window'][field] != target[field] for field in ('id', 'pid')):
        raise ValueError('The selected host window changed. Capture the game again.')
    return target


def literal_text(config, source):
    if source == '-':
        data = sys.stdin.buffer.read(MAX_TEXT_BYTES + 1)
    else:
        path = checked_file(source)
        info = path.stat()
        if info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) & PRIVATE_PERMISSION_MASK:
            raise ValueError('The text file must belong to this user and have private permissions.')
        with path.open('rb') as stream:
            data = stream.read(MAX_TEXT_BYTES + 1)
    if not data or len(data) > MAX_TEXT_BYTES:
        raise ValueError(f'Text must contain between 1 and {MAX_TEXT_BYTES} UTF-8 bytes.')
    value = data.decode('utf-8')
    if any(not character.isprintable() for character in value):
        raise ValueError('Text must not contain control characters or line breaks.')
    wine_input(config, [], data)
    print(json.dumps(dict(operation='text', submitted=True, visible_result_verified=False)))


def wheel_input(config, direction, count):
    if direction not in ('up', 'down') or not isinstance(count, int) or isinstance(count, bool) or not 1 <= count <= MAX_WHEEL_COUNT:
        raise ValueError(f'Wheel input requires up or down and a count from 1 to {MAX_WHEEL_COUNT}.')
    wine_input(config, ['wheel', direction, count])
    print(json.dumps(dict(operation='wheel', direction=direction, count=count,
                          submitted=True, visible_result_verified=False)))


def restore_window(config):
    wine_input(config, ['restore', 'visible', 'only'], allow_missing_window=True)
    print(json.dumps(dict(operation='restore', submitted=True, visible_result_verified=False)))


def drag_input(config, dx, dy, seconds):
    duration = float(seconds)
    if any(not isinstance(value, int) or isinstance(value, bool) or abs(value) > MAX_DRAG_PIXELS for value in (dx, dy)) or (dx == 0 and dy == 0):
        raise ValueError(f'Drag displacement must use integer pixels within +/-{MAX_DRAG_PIXELS} and must not be zero.')
    if not math.isfinite(duration) or not 0 < duration <= MAX_KEY_HOLD_SECONDS:
        raise ValueError(f'Drag duration must be greater than zero and at most {MAX_KEY_HOLD_SECONDS} seconds.')
    milliseconds = round(duration * MILLISECONDS_PER_SECOND)
    if milliseconds <= 0:
        raise ValueError('Drag duration must be at least one millisecond.')
    wine_input(config, ['drag', dx, dy, milliseconds])
    print(json.dumps(dict(operation='drag', dx=dx, dy=dy, hold_seconds=duration,
                          submitted=True, visible_result_verified=False)))


def credential(path):
    info = path.stat()
    if info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) & PRIVATE_PERMISSION_MASK:
        raise ValueError('The Jev key file must belong to this user and have permissions 0600 or 0400.')
    value = path.read_text().strip()
    if not value or '\n' in value or '\r' in value:
        raise ValueError('The Jev key file is empty or invalid.')
    return value


class DecisionFailure(ValueError):
    def __init__(self, status, diagnostic):
        super().__init__(diagnostic)
        self.status = status


def jev_credential(config):
    try:
        return credential(Path(config['jev_key_file']))
    except (OSError, ValueError, KeyError):
        raise DecisionFailure('credential_error', 'Jev credentials are absent, unreadable, or not private.') from None


def jev_decide(config, image, character, task='login'):
    image = image.expanduser().resolve()
    png_size(image)
    with tempfile.TemporaryDirectory() as temporary:
        source = Path(temporary) / 'source.png'
        source_data = image.read_bytes()
        source_hash = hashlib.sha256(source_data).hexdigest()
        source.write_bytes(source_data)
        resized = Path(temporary) / 'screen.png'
        run(['sips', '-Z', MAX_IMAGE_EDGE, source, '--out', resized], capture_output=True)
        if min(png_size(resized)) < MIN_IMAGE_EDGE:
            raise ValueError('The image is too small for Jev. Each edge must be at least 280 pixels.')
        image_data = resized.read_bytes()
    reference = 'game-screen'
    body = {'model': config['jev_model'], 'state': {'screen_image': reference, 'character': character},
            'assets': {reference: {'type': 'image', 'format': 'png',
                                  'data_base64': base64.b64encode(image_data).decode()}},
            'questions': {
                'screen': {'type': 'choice', 'instructions': 'Classify only the visible screen_image.',
                           'criteria': {'agreement': 'A user agreement or consent prompt.',
                                        'character_menu': 'The title menu with Select Character highlighted.',
                                        'character_select': 'The character list.',
                                        'character_confirm': 'A local character login confirmation.',
                                        'loading': 'The game is loading or connecting.',
                                        'gameplay': 'A character is inside a game zone with a gameplay HUD.',
                                        'error': 'An error dialog.', 'other': 'An unclear or different screen.'}},
                'next_key': {'type': 'choice',
                             'instructions': 'Propose one visible action. Do not send input or decide authorization. '
                                             'Use state.character as the exact character name.',
                             'criteria': {'enter': 'Select Character is highlighted, or the exact requested character '
                                                  'is highlighted, Yes is highlighted for that character login, or the agreement continue action is selected.',
                                          'wait': 'Loading or connecting requires no input.',
                                          'stop': 'Gameplay, error, different character, or unclear action.'}}}}
    if task == 'dialogue':
        body['state']['task'] = task
        body['questions']['screen']['criteria'] = {
            'dialogue': 'NPC or story text with a downward advance marker and no selectable answers. Spoken questions and quoted questions remain dialogue. The HUD may remain visible.',
            'dialogue_typing': 'Plain NPC or story dialogue is still revealing text and has no advance marker yet. No selectable options or consent prompt.',
            'choice': 'Selectable answers or menu rows with a highlighted option or selection cursor. A question inside spoken NPC text is not a choice.',
            'agreement': 'A user agreement or consent prompt.',
            'loading': 'A loading or connecting screen without dialogue.',
            'gameplay': 'A controllable character in a zone without active story or NPC dialogue.',
            'error': 'An error dialog.',
            'other': 'An unclear or different screen.'}
        body['questions']['next_key']['instructions'] = ('Propose one visible action for this dialogue task. '
            'Use Enter for plain dialogue with an advance marker or a selected agreement continue action. '
            'Use Wait for loading or text that is still appearing. Otherwise use Stop. '
            'Do not send input or decide authorization.')
        body['questions']['next_key']['criteria'] = {
            'enter': 'Plain dialogue has an advance marker, or the agreement continue action is selected.',
            'wait': 'Loading or dialogue text is still appearing.',
            'stop': 'Choices, gameplay, errors, or an unclear action.'}
    request = urllib.request.Request(JEV_URL, data=json.dumps(body).encode(),
                                     headers={'Authorization': 'Bearer ' + jev_credential(config),
                                              'Content-Type': 'application/json',
                                              'TypeSafe-Danger-Zone-Images': 'true'})
    started = time.monotonic()
    try:
        with urllib.request.urlopen(request, timeout=COMMAND_TIMEOUT_SECONDS) as response:
            result = json.load(response)
    except urllib.error.HTTPError as error:
        raise DecisionFailure('api_error', f'Jev returned HTTP {error.code}. No input was sent.') from None
    if not isinstance(result, dict) or not isinstance(result.get('answers'), dict):
        raise DecisionFailure('schema_error', 'Jev answers must contain screen and next_key objects.')
    answers = result['answers']
    if task == 'dialogue' and isinstance(answers.get('screen'), dict) and answers['screen'].get('choice') != 'agreement':
        screen = answers['screen']
        action = {'dialogue': 'enter', 'dialogue_typing': 'wait', 'loading': 'wait'}.get(screen.get('choice'), 'stop')
        answers['next_key'] = dict(type='choice', choice=action, confidence=screen.get('confidence'),
                                   policy='dialogue-screen')
    if any(not isinstance(answers.get(field), dict) for field in ('screen', 'next_key')):
        raise DecisionFailure('schema_error', 'Jev answers must contain screen and next_key objects.')
    if answers['next_key'].get('choice') not in ('enter', 'wait', 'stop'):
        raise DecisionFailure('schema_error', 'Jev next_key must be enter, wait, or stop.')
    output = dict(image=str(image), image_sha256=source_hash,
                  submitted_sha256=hashlib.sha256(image_data).hexdigest(),
                  seconds=round(time.monotonic() - started, 3), response=result)
    target = image.with_suffix('.jev.json')
    private_write(target, (json.dumps(output, indent=2) + '\n').encode())
    return output


SCREENS = ('agreement', 'character_menu', 'character_select', 'character_confirm',
           'loading', 'gameplay', 'error', 'other')
DECISION_SCHEMA = {
    'type': 'object', 'additionalProperties': False,
    'required': ['image_sha256', 'screen', 'next_key', 'confidence'],
    'properties': {'image_sha256': {'type': 'string'},
                   'screen': {'enum': list(SCREENS)},
                   'next_key': {'enum': ['enter', 'wait', 'stop']},
                   'confidence': {'type': 'number', 'minimum': 0, 'maximum': 1}}}


def validate_decision(result, image, task='login'):
    if set(result) != set(DECISION_SCHEMA['required']):
        raise ValueError('Decision fields do not match the requested schema.')
    if result['image_sha256'] != digest(image):
        raise ValueError('The decision does not match this capture.')
    screens = SCREENS + ('dialogue', 'dialogue_typing', 'choice') if task == 'dialogue' else SCREENS
    if result['screen'] not in screens or result['next_key'] not in ('enter', 'wait', 'stop'):
        raise ValueError('Unsupported screen or action.')
    confidence = result['confidence']
    if type(confidence) not in (int, float) or not math.isfinite(confidence) or not 0 <= confidence <= 1:
        raise ValueError('Invalid decision confidence.')
    enter_screens = ('dialogue',) if task == 'dialogue' else ('character_menu', 'character_select', 'character_confirm')
    if result['next_key'] == 'enter' and (confidence < DECISION_CONFIDENCE_THRESHOLD or result['screen'] not in enter_screens):
        raise ValueError('Enter is not permitted for this screen or confidence.')
    wait_screens = ('loading', 'dialogue_typing') if task == 'dialogue' else ('loading',)
    if result['next_key'] == 'wait' and result['screen'] not in wait_screens:
        raise ValueError('Wait is not permitted for this screen.')
    return result


def caller_policy(decision, agreement_policy='caller'):
    if agreement_policy not in ('caller', 'permitted', 'confirmation_required'):
        raise DecisionFailure('policy_error', 'Unsupported caller agreement policy.')
    if decision['screen'] != 'agreement' or agreement_policy == 'permitted':
        return dict(status='permitted', reason='caller_authorized' if decision['screen'] == 'agreement' else 'routine_task')
    if agreement_policy == 'confirmation_required':
        return dict(status='policy_required', reason='active_harness_policy',
                    diagnostic='The active harness requires confirmation for this agreement action.')
    return dict(status='caller_policy_required', reason='policy_not_supplied',
                diagnostic='The caller must apply its active harness policy to the proposed agreement action.')


def validate_proposal(decision, image, task):
    if not isinstance(decision, dict) or set(decision) != set(DECISION_SCHEMA['required']):
        raise DecisionFailure('schema_error', 'Decision fields do not match the requested schema.')
    if decision['image_sha256'] != digest(image):
        raise DecisionFailure('capture_changed', 'The capture changed after classification. Capture a fresh image.')
    screens = SCREENS + ('dialogue', 'dialogue_typing', 'choice') if task == 'dialogue' else SCREENS
    if decision['screen'] not in screens or decision['next_key'] not in ('enter', 'wait', 'stop'):
        raise DecisionFailure('schema_error', 'The screen or proposed action is unsupported.')
    confidence = decision['confidence']
    if type(confidence) not in (int, float) or not math.isfinite(confidence) or not 0 <= confidence <= 1:
        raise DecisionFailure('schema_error', 'Confidence must be a finite number from zero to one.')
    return decision


def vision_request(image, character, task):
    schema = {**DECISION_SCHEMA, 'properties': {**DECISION_SCHEMA['properties']}}
    if task == 'dialogue':
        schema['properties']['screen'] = {'enum': list(SCREENS) + ['dialogue', 'dialogue_typing', 'choice']}
    return dict(status='vision_requested', provider='vision', reason='explicit_selection',
                image=str(image), image_sha256=digest(image), character=character, schema=schema,
                instructions='Classify the capture and propose one action. Return the requested JSON. '
                             'Do not send input or decide authorization. The caller applies its active harness policy. '
                             'Validate the result with decide --result and the same task before use.')


def decide(config, args):
    image = args.image.expanduser().resolve()
    png_size(image)
    task = getattr(args, 'task', 'login')
    if task not in ('login', 'dialogue'):
        raise ValueError('Unsupported decision task.')
    if args.vision and not args.result:
        output = vision_request(image, args.character, task)
    else:
        provider = 'vision' if args.result else 'jev'
        try:
            seconds = None
            if args.result:
                decision = json.loads(args.result.read_text())
            else:
                result = jev_decide(config, image, args.character, task)
                answers = result['response']['answers']
                confidences = [answers[field]['confidence'] for field in ('screen', 'next_key')]
                if any(type(value) not in (int, float) or not math.isfinite(value) or not 0 <= value <= 1
                       for value in confidences):
                    raise DecisionFailure('schema_error', 'Jev confidence must be a finite number from zero to one.')
                decision = dict(image_sha256=result['image_sha256'], screen=answers['screen']['choice'],
                                next_key=answers['next_key']['choice'], confidence=min(confidences))
                seconds = result['seconds']
            validate_proposal(decision, image, task)
            if decision['confidence'] < DECISION_CONFIDENCE_THRESHOLD:
                output = dict(status='low_confidence', provider=provider, decision=decision, image=str(image),
                              diagnostic=f"Confidence {decision['confidence']} is below {DECISION_CONFIDENCE_THRESHOLD}.",
                              next_step='Capture a fresh frame or select native vision explicitly.')
            else:
                policy = caller_policy(decision, getattr(args, 'agreement_policy', 'caller'))
                if policy['status'] == 'permitted' and decision['screen'] != 'agreement':
                    validate_decision(decision, image, task)
                output = dict(status=policy['status'] if policy['status'] != 'permitted' else 'decision',
                              provider=provider, decision=decision, caller_policy=policy, image=str(image), seconds=seconds)
        except DecisionFailure as error:
            output = dict(status=error.status, provider=provider, diagnostic=str(error), image=str(image))
        except (TimeoutError, urllib.error.URLError) as error:
            output = dict(status='api_error', provider=provider, diagnostic=type(error).__name__ + ': Jev request failed.', image=str(image))
        except ValueError as error:
            output = dict(status='schema_error', provider=provider, diagnostic=str(error), image=str(image))
        except (KeyError, TypeError) as error:
            output = dict(status='schema_error', provider=provider,
                          diagnostic=type(error).__name__ + ': Required decision fields are absent or have invalid types.', image=str(image))
        except (OSError, subprocess.SubprocessError) as error:
            output = dict(status='local_error', provider=provider,
                          diagnostic=type(error).__name__ + ': Local image operation failed.', image=str(image))
    output['image_sha256'] = digest(image)
    output['task'] = task
    private_write(image.with_suffix('.decision.json'), (json.dumps(output, indent=2) + '\n').encode())
    print(json.dumps(output, indent=2))
    return output


def dialogue_fingerprint(image):
    width, height = png_size(image)
    result = run(['swift', SCRIPT_DIR / 'lib/ocr-macos.swift', image], capture_output=True, text=True)
    text = []
    for line in result.stdout.splitlines():
        fields = line.rsplit('\t', 2)
        if len(fields) != 3:
            continue
        value, x, y = fields
        if float(x) < width * DIALOGUE_TEXT_RIGHT_FRACTION and float(y) >= height * DIALOGUE_TEXT_TOP_FRACTION:
            text.append(re.sub(r'[^a-z0-9]+', '', value.lower()))
    value = ''.join(text)
    if not value:
        raise ValueError('The dialogue text could not be read. No input was sent.')
    return hashlib.sha256(value.encode()).hexdigest()


def drive_dialogue(config, args):
    if not 1 <= args.max_actions <= MAX_DIALOGUE_ACTIONS or not 1 <= args.max_seconds <= MAX_DIALOGUE_SECONDS:
        raise ValueError('The dialogue action or time limit is outside the supported range.')
    folder = args.folder.expanduser().resolve()
    folder.mkdir(parents=True, exist_ok=False, mode=0o700)
    deadline = time.monotonic() + args.max_seconds
    result = dict(status='stopped', actions=0, captures=[], reason='time_limit')
    try:
        drive_dialogue_steps(config, args, folder, result, deadline)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        result.update(status='blocked', reason='operation_failed', diagnostic=str(error))
    private_write(folder / 'summary.json', (json.dumps(result, indent=2) + '\n').encode())
    print(json.dumps(result, indent=2))
    return result


def drive_dialogue_steps(config, args, folder, result, deadline):
    previous = None
    unchanged = 0
    low_confidence_retries = 0
    while time.monotonic() < deadline:
        image = capture(config, folder / f'frame-{len(result["captures"]):03}.png')
        decision_args = argparse.Namespace(image=image, character=args.character, task='dialogue', result=None, vision=False,
                                           agreement_policy=getattr(args, 'agreement_policy', 'caller'))
        decide(config, decision_args)
        output = json.loads(image.with_suffix('.decision.json').read_text())
        entry = dict(image=str(image), image_sha256=digest(image), decision=str(image.with_suffix('.decision.json')))
        result['captures'].append(entry)
        decision = validate_proposal(output['decision'], image, 'dialogue') if output['status'] == 'decision' else None
        if output['status'] == 'low_confidence' or (decision and decision['confidence'] < DECISION_CONFIDENCE_THRESHOLD):
            if low_confidence_retries >= MAX_LOW_CONFIDENCE_RETRIES:
                result['reason'] = 'low_confidence'
                result['diagnostic'] = output.get('diagnostic', 'The controller rejected a decision below the confidence threshold.')
                break
            low_confidence_retries += 1
            entry['recovery'] = 'fresh_capture_without_input'
            time.sleep(DIALOGUE_WAIT_SECONDS)
            continue
        if output['status'] != 'decision':
            result['reason'] = output['status']
            result['diagnostic'] = output.get('diagnostic', output.get('caller_policy', {}).get('diagnostic'))
            break
        low_confidence_retries = 0
        policy = caller_policy(decision, getattr(args, 'agreement_policy', 'caller'))
        if policy['status'] != 'permitted':
            result['reason'] = policy['status']
            result['diagnostic'] = policy['diagnostic']
            break
        if decision['screen'] != 'agreement':
            validate_decision(decision, image, 'dialogue')
        if decision['next_key'] == 'stop':
            result['reason'] = decision['screen']
            break
        if decision['next_key'] == 'wait':
            time.sleep(DIALOGUE_WAIT_SECONDS)
            continue
        fingerprint = dialogue_fingerprint(image)
        entry['dialogue_fingerprint'] = fingerprint
        unchanged = unchanged + 1 if fingerprint == previous else 0
        if unchanged >= MAX_UNCHANGED_DIALOGUE_OUTCOMES:
            result['reason'] = 'unchanged_dialogue'
            break
        if result['actions'] >= args.max_actions or time.monotonic() >= deadline:
            result['reason'] = 'action_or_time_limit'
            break
        previous = fingerprint
        input_config = {**config, 'input_frame': image} if config.get('offscreen_control') else config
        key(input_config, 'enter', DEFAULT_HOLD_SECONDS)
        result['actions'] += 1
        private_write(folder / 'summary.json', (json.dumps(result, indent=2) + '\n').encode())
        time.sleep(DIALOGUE_WAIT_SECONDS)


def main():
    parser = argparse.ArgumentParser(description='Configure and drive the FFXI Wine client on macOS.')
    parser.add_argument('--profile', default='macos')
    parser.add_argument('--offscreen', action='store_true')
    parser.add_argument('--frame', type=Path)
    sub = parser.add_subparsers(dest='command', required=True)
    setup_parser = sub.add_parser('setup')
    for option in ('runner', 'prefix', 'install', 'loader'):
        setup_parser.add_argument('--' + option, required=True)
    setup_parser.add_argument('--server', default='127.0.0.1')
    setup_parser.add_argument('--startup-enter', action='store_true', help='Enable one startup Enter for this local private-server profile')
    setup_parser.add_argument('--loader-arg', action='append', default=[])
    setup_parser.add_argument('--jev-key-file', default='~/.config/typesafe/api-key')
    setup_parser.add_argument('--ahk-archive')
    setup_parser.add_argument('--replace', action='store_true')
    sub.add_parser('doctor')
    capture_parser = sub.add_parser('capture')
    capture_parser.add_argument('image', type=Path)
    key_parser = sub.add_parser('key')
    key_parser.add_argument('name')
    key_parser.add_argument('seconds', nargs='?', default=DEFAULT_HOLD_SECONDS)
    text_parser = sub.add_parser('text')
    text_parser.add_argument('source', help='Private UTF-8 file, or - for standard input')
    wheel_parser = sub.add_parser('wheel')
    wheel_parser.add_argument('direction', choices=('up', 'down'))
    wheel_parser.add_argument('count', type=int, nargs='?', default=1)
    drag_parser = sub.add_parser('drag')
    drag_parser.add_argument('dx', type=int)
    drag_parser.add_argument('dy', type=int)
    drag_parser.add_argument('seconds', nargs='?', default=DEFAULT_HOLD_SECONDS)
    decide_parser = sub.add_parser('decide')
    decide_parser.add_argument('image', type=Path)
    decide_parser.add_argument('--character', required=True)
    decide_parser.add_argument('--task', choices=('login', 'dialogue'), default='login')
    decide_parser.add_argument('--vision', action='store_true')
    decide_parser.add_argument('--agreement-policy', choices=('caller', 'permitted', 'confirmation_required'), default='caller')
    decide_parser.add_argument('--result', type=Path)
    dialogue_parser = sub.add_parser('drive-dialogue')
    dialogue_parser.add_argument('folder', type=Path)
    dialogue_parser.add_argument('--character', required=True)
    dialogue_parser.add_argument('--agreement-policy', choices=('caller', 'permitted', 'confirmation_required'), default='caller')
    dialogue_parser.add_argument('--max-actions', type=int, default=MAX_DIALOGUE_ACTIONS)
    dialogue_parser.add_argument('--max-seconds', type=int, default=MAX_DIALOGUE_SECONDS)
    sub.add_parser('restore')
    launch_parser = sub.add_parser('launch')
    launch_parser.add_argument('--agreement-policy', choices=('caller', 'permitted', 'confirmation_required'))
    args = parser.parse_args()
    if sys.platform != 'darwin':
        raise ValueError('This command requires macOS.')
    path = profile_path(args.profile)
    if args.command == 'setup':
        setup(args, path)
        return
    config = load(path)
    config = {**config, 'offscreen_control': args.offscreen, 'input_frame': args.frame}
    if args.command == 'doctor':
        for command in ('python3', 'bash', 'osascript', 'screencapture', 'sips', 'ioreg', 'caffeinate'):
            if not shutil.which(command):
                raise ValueError(f'Required command is missing: {command}')
        permissions()
        unlocked()
        print(json.dumps(dict(window=window(config, allow_missing=True))))
        if platform.machine() == 'arm64':
            run(['arch', '-x86_64', '/usr/bin/true'], capture_output=True)
            print('Rosetta execution passed.')
        print(f"Wine: {config['runner_version']}")
        print(f"Client SHA256: {config['client_sha256']}")
        print('Input dependencies passed. Input delivery still requires a visible check.')
        print('If no window was found, launch the client before the visible check.')
        if Path(config['jev_key_file']).exists():
            try:
                credential(Path(config['jev_key_file']))
                print('Jev credential permissions passed. API access is checked only by decide.')
            except (OSError, ValueError):
                print('Jev credential is unavailable. Decide will report credential_error.')
        else:
            print('Jev credential is absent. Capture and input do not require Jev.')
    elif args.command == 'capture':
        capture(config, args.image)
    elif args.command == 'key':
        key(config, args.name, args.seconds)
    elif args.command == 'text':
        literal_text(config, args.source)
    elif args.command == 'wheel':
        wheel_input(config, args.direction, args.count)
    elif args.command == 'drag':
        drag_input(config, args.dx, args.dy, args.seconds)
    elif args.command == 'decide':
        decide(config, args)
    elif args.command == 'drive-dialogue':
        if drive_dialogue(config, args)['status'] == 'blocked':
            raise ValueError('The dialogue controller stopped after an operation failure. Inspect its summary.')
    elif args.command == 'restore':
        restore_window(config)
    elif args.command == 'launch':
        launch(config, args)


if __name__ == '__main__':
    try:
        main()
    except subprocess.CalledProcessError as error:
        print(f'macos-observe: A required command failed (exit {error.returncode}). No command arguments were logged.', file=sys.stderr)
        raise SystemExit(1)
    except (OSError, ValueError, KeyError, subprocess.TimeoutExpired, zipfile.BadZipFile) as error:
        print(f'macos-observe: {error}', file=sys.stderr)
        raise SystemExit(1)
