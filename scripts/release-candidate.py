#!/usr/bin/env python3
import argparse
import re
import subprocess


def resolve(tag, expected_sha, publish):
    if not re.fullmatch(r'v(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?', tag):
        raise ValueError('Release tag must be a v-prefixed version')
    if not re.fullmatch(r'[0-9a-f]{40}', expected_sha):
        raise ValueError('Expected commit must be a full SHA')
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    if head != expected_sha:
        raise ValueError('Checkout differs from workflow commit')
    tagged = subprocess.run(['git', 'rev-parse', '--verify', f'refs/tags/{tag}^{{commit}}'], capture_output=True, text=True)
    if tagged.returncode == 0:
        if tagged.stdout.strip() != head:
            raise ValueError('Requested tag differs from workflow commit')
    elif publish:
        raise ValueError('Publishing requires an existing tag at the workflow commit')
    return {'tag': tag, 'sha': head, 'publish': str(publish).lower()}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--tag', required=True)
    parser.add_argument('--sha', required=True)
    parser.add_argument('--publish', choices=['true', 'false'], required=True)
    args = parser.parse_args()
    for key, value in resolve(args.tag, args.sha, args.publish == 'true').items():
        print(f'{key}={value}')


if __name__ == '__main__':
    main()
