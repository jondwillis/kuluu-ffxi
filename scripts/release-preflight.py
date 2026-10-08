#!/usr/bin/env python3
import argparse
import datetime as dt
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
from zoneinfo import ZoneInfo

REPO = 'jondwillis/kuluu-ffxi'
SCHEMA = 1
POLICY = 'shadow-v1'
SHA = re.compile(r'[0-9a-f]{40}')
TAG = re.compile(r'v\d+\.\d+\.\d+')


def api(endpoint):
    result = subprocess.run(['gh', 'api', f'repos/{REPO}/{endpoint}'],
                            check=True, capture_output=True, text=True, timeout=60)
    return json.loads(result.stdout)


def snapshot():
    release = api('releases/latest')
    if release['draft'] or release['prerelease']:
        raise ValueError('Latest release must be published and stable')
    tag = release['tag_name']
    if not TAG.fullmatch(tag):
        raise ValueError('Latest stable tag is not a supported version')
    return {'repo': REPO, 'main_sha': api('commits/main')['sha'],
            'stable_sha': api(f'commits/{tag}')['sha'], 'stable_tag': tag,
            'release_updated_at': release['updated_at']}


def candidate(data, day):
    if data['repo'] != REPO or not TAG.fullmatch(data['stable_tag']):
        raise ValueError('Unexpected repository or stable tag')
    for field in ('main_sha', 'stable_sha'):
        if not SHA.fullmatch(data[field]):
            raise ValueError(f'Invalid {field}')
    scope = 'weekly-features' if day.weekday() == 2 else 'daily-hotfixes'
    inputs = {**data, 'policy': POLICY, 'scope': scope}
    key = hashlib.sha256(json.dumps(inputs, sort_keys=True).encode()).hexdigest()
    return key, scope


def write_state(path, value):
    with tempfile.NamedTemporaryFile(mode='w', dir=path.parent, delete=False) as f:
        temp = Path(f.name)
        json.dump(value, f, sort_keys=True, indent=2)
        f.write('\n')
        f.flush()
        os.fsync(f.fileno())
    os.replace(temp, path)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--state', type=Path, required=True)
    parser.add_argument('--snapshot', type=Path, help='Offline rehearsal inputs; never use as live evidence')
    parser.add_argument('--date', type=dt.date.fromisoformat, help='Rehearsal date override')
    parser.add_argument('--signals', type=Path, help='Validation status JSON; changes reopen held candidates')
    parser.add_argument('--ack', help='Exact key returned after completing an assessment')
    parser.add_argument('--decision', choices=['hold', 'prepared', 'no-eligible-fixes'])
    parser.add_argument('--evidence', type=Path, help='Existing assessment JSON, bound to this key')
    args = parser.parse_args()
    if args.date and not args.snapshot:
        parser.error('--date requires --snapshot')
    if any((args.ack, args.decision, args.evidence)) and not all((args.ack, args.decision, args.evidence)):
        parser.error('--ack requires --decision and --evidence together')
    args.state.parent.mkdir(parents=True, exist_ok=True)
    with args.state.with_suffix(args.state.suffix + '.lock').open('w') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        state = json.loads(args.state.read_text()) if args.state.exists() else {'schema': SCHEMA, 'assessments': {}}
        if state['schema'] != SCHEMA or not isinstance(state['assessments'], dict):
            raise ValueError('Unsupported ledger; preserve it and reconcile manually')
        data = json.loads(args.snapshot.read_text()) if args.snapshot else snapshot()
        if args.signals:
            signals = json.loads(args.signals.read_text())
            data['validation_signal_sha256'] = hashlib.sha256(
                json.dumps(signals, sort_keys=True).encode()).hexdigest()
        day = args.date or dt.datetime.now(ZoneInfo('America/Los_Angeles')).date()
        key, scope = candidate(data, day)
        if args.ack:
            if args.ack != key:
                raise ValueError('Candidate changed; assessment was not acknowledged')
            evidence = json.loads(args.evidence.read_text())
            if evidence.get('key') != key or evidence.get('decision') != args.decision or not evidence.get('summary'):
                raise ValueError('Assessment must name this key, decision, and nonempty summary')
            state['assessments'][key] = {'decision': args.decision, 'evidence': str(args.evidence.resolve()),
                'evidence_sha256': hashlib.sha256(args.evidence.read_bytes()).hexdigest(),
                'recorded_at': dt.datetime.now(dt.timezone.utc).isoformat()}
            write_state(args.state, state)
            status = 'recorded'
        elif data['main_sha'] == data['stable_sha']:
            status = 'no-change'
        elif key in state['assessments']:
            prior = state['assessments'][key]
            artifact = Path(prior['evidence'])
            intact = artifact.is_file() and hashlib.sha256(artifact.read_bytes()).hexdigest() == prior['evidence_sha256']
            status = 'unchanged' if intact else 'assessment-required'
        else:
            status = 'assessment-required'
        print(json.dumps({'schema': SCHEMA, 'mode': 'shadow', 'status': status,
            'scope': scope, 'key': key, 'inputs': data, 'publish_allowed': False,
            'rehearsal': bool(args.snapshot)}))


if __name__ == '__main__':
    main()
