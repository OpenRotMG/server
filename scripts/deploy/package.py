#!/usr/bin/env python3
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile

sys.dont_write_bytecode = True
from realm_contract import check_realms
revision, binary, output = sys.argv[1:]
assert subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip() == revision
output = Path(output)
output.mkdir()
with tempfile.TemporaryDirectory() as temporary:
    root = Path(temporary)
    subprocess.run(['git', 'archive', revision, '-o', str(root / 'source.tar')], check=True)
    stage = root / 'server'
    stage.mkdir()
    with tarfile.open(root / 'source.tar') as archive:
        archive.extractall(stage, filter='data')
    shutil.copyfile(binary, stage / 'openrotmg')
    (stage / 'openrotmg').chmod(0o755)
    (stage / 'REVISION').write_text(revision + '\n')
    sys.path.insert(0, str(stage / 'scripts/backups'))
    from release_contract import check
    check(stage / 'openrotmg', stage / 'data', root / 'contract', revision)
    check_realms(stage / 'openrotmg', stage / 'data', root / 'realm-data')
    digest = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
    manifest = {str(p.relative_to(stage)): digest(p) for p in sorted(stage.rglob('*')) if p.is_file()}
    with tarfile.open(output / 'server.tar.gz', 'w:gz') as archive:
        for p in sorted(stage.rglob('*')):
            if p.is_file():
                archive.add(p, arcname=str(p.relative_to(stage)))
    metadata = {'format': 1, 'revision': revision, 'archive_sha256': digest(output / 'server.tar.gz'),
                'binary_sha256': manifest['openrotmg'], 'manifest': manifest}
    (output / 'release.json').write_text(json.dumps(metadata, sort_keys=True) + '\n')
