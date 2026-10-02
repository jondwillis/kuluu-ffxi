#!/usr/bin/env python3
import argparse
from html import unescape
from html.parser import HTMLParser
from pathlib import Path
import posixpath
import re
import subprocess
import sys
from urllib.parse import unquote, urlsplit


MAX_WORDS = 1200
MAX_CODE_LINES = 35
PUBLIC_GUIDES = (
    'README.md', 'CONTRIBUTING.md', 'SUPPORT.md', 'vendor/README.md',
    'kuluu/assets/branding/README.md',
)


class Links(HTMLParser):
    def __init__(self):
        super().__init__()
        self.targets = []
        self.anchors = set()

    def handle_starttag(self, tag, attrs):
        for key, value in attrs:
            if value and key in ('href', 'src'):
                self.targets.append(value)
            if value and (key == 'id' or (tag == 'a' and key == 'name')):
                self.anchors.add(value)


def without_code(text):
    return re.sub(r'^(`{3,}|~{3,}).*?^\1[^\n]*$', '', text, flags=re.M | re.S)


def markdown_anchors(text):
    anchors = set()
    counts = {}
    for title in re.findall(r'^#{1,6}\s+(.+?)\s*#*$', without_code(text), re.M):
        title = re.sub(r'<[^>]*>', '', title).lower()
        slug = re.sub(r'[^\w\- ]', '', title).replace(' ', '-')
        count = counts.get(slug, 0)
        counts[slug] = count + 1
        anchors.add(f'{slug}-{count}' if count else slug)
    return anchors


def editorial_errors(text):
    visible = re.sub(r'<!--.*?-->', '', text, flags=re.S)
    visible = re.sub(r'<[^>]*>', '', visible)
    visible = re.sub(r'!?\[([^\]]*)\]\([^\n)]*\)', r'\1', visible)
    words = len(visible.split())
    errors = []
    if words > MAX_WORDS:
        errors.append(f'README.md: {words} words exceeds {MAX_WORDS}; move specialist detail to a guide')
    code_lines = sum(
        bool(line.strip())
        for block in re.findall(r'^(`{3,}|~{3,})[^\n]*\n(.*?)^\1[^\n]*$', text, re.M | re.S)
        for line in block[1].splitlines()
    )
    if code_lines > MAX_CODE_LINES:
        errors.append(f'README.md: {code_lines} code lines exceeds {MAX_CODE_LINES}; link to command reference')
    if re.search(r'\b(?:kuluu|bd)-(?!ffxi\b)[a-z0-9]{4}(?:\.[0-9]+)?\b(?!-)', text):
        errors.append('README.md: internal task ID; link to a public issue instead')
    return errors


class Tree:
    def __init__(self, staged=False):
        self.staged = staged
        self.files = set(subprocess.check_output(['git', 'ls-files', '-z'], text=True).split('\0'))

    def read(self, path):
        if self.staged:
            return subprocess.check_output(['git', 'show', f':{path}'], text=True, stderr=subprocess.DEVNULL)
        return Path(path).read_text()

    def exists(self, path):
        if self.staged:
            return path in self.files or any(p.startswith(path.rstrip('/') + '/') for p in self.files)
        return Path(path).exists()


def link_errors(path, text, tree):
    text = without_code(text)
    html = Links()
    html.feed(text)
    targets = html.targets + re.findall(r'!?\[[^\]]*\]\(([^\s)]+)(?:\s+[^)]*)?\)', text)
    targets += re.findall(r'^\[[^\]]+\]:\s*(\S+)', text, re.M)
    errors = []
    for target in targets:
        url = urlsplit(unescape(target.strip('<>')))
        if url.scheme or url.netloc:
            continue
        dest = posixpath.normpath(posixpath.join(posixpath.dirname(path), unquote(url.path))) if url.path else path
        if not tree.exists(dest):
            errors.append(f'{path}: missing local target {target}')
        elif url.fragment and dest.endswith('.md'):
            content = tree.read(dest)
            ids = Links()
            ids.feed(content)
            if unquote(url.fragment) not in markdown_anchors(content) | ids.anchors:
                errors.append(f'{path}: missing heading {target}')
    return errors


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--staged', action='store_true')
    args = parser.parse_args()
    tree = Tree(args.staged)
    errors = []
    for path in PUBLIC_GUIDES:
        try:
            text = tree.read(path)
            if path == 'README.md':
                errors.extend(editorial_errors(text))
            errors.extend(link_errors(path, text, tree))
        except (OSError, subprocess.CalledProcessError) as error:
            errors.append(f'{path}: cannot read guide or linked file: {error}')
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        print('See CONTRIBUTING.md#readme-editorial-policy', file=sys.stderr)
    else:
        print('README scope and public-guide local links: ok')
    return bool(errors)


if __name__ == '__main__':
    sys.exit(main())
