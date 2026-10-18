import hashlib
import json
from pathlib import Path
import shutil
import subprocess


def check_realms(binary, data, directory):
    binary, data, directory = Path(binary).resolve(), Path(data).resolve(), Path(directory)
    shutil.copytree(data, directory)
    recipes = []
    for entry in json.loads((directory / 'realm-catalog.json').read_text()):
        if entry['kind'] == 'baseline':
            output = directory / entry['path']
        elif entry['kind'] == 'reconstruction':
            recipe = json.loads((directory / 'realms' / entry['recipe']).read_text())
            output = directory / 'realms' / recipe['output']
            recipes.append((recipe['id'], output, entry['sha256']))
        else:
            raise RuntimeError(f"Unknown Realm catalog kind: {entry['kind']}")
        if hashlib.sha256(output.read_bytes()).hexdigest() != entry['sha256']:
            raise RuntimeError(f'Realm catalog hash mismatch: {output}')
    if not recipes:
        raise RuntimeError('Realm catalog has no reconstruction recipes')
    # Remove every built layout together so no reconstruction can depend on another output.
    for _, output, _ in recipes:
        output.unlink()
    for realm_id, output, expected in recipes:
        command = [str(binary), 'build-realm', '--data', str(directory), '--id', realm_id]
        subprocess.run(command, check=True)
        if hashlib.sha256(output.read_bytes()).hexdigest() != expected:
            raise RuntimeError(f'Rebuilt Realm differs from committed bytes: {realm_id}')
        subprocess.run([*command, '--check'], check=True)
    print(json.dumps({'event': 'release_realm_contract_passed',
                      'ids': [realm_id for realm_id, _, _ in recipes]}))
