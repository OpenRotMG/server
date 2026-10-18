import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from realm_contract import check_realms


class RealmContractTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.data = self.root / 'data'
        (self.data / 'realms').mkdir(parents=True)
        self.catalog = []
        self.outputs = {}
        for realm_id in ['one', 'two']:
            output = f'{realm_id}.wmap'
            value = realm_id.encode()
            self.outputs[realm_id] = value
            (self.data / 'realms' / output).write_bytes(value)
            recipe = {'id': realm_id, 'output': output}
            (self.data / 'realms' / f'{realm_id}.json').write_text(json.dumps(recipe))
            self.catalog.append({'kind': 'reconstruction', 'recipe': f'{realm_id}.json',
                                 'sha256': hashlib.sha256(value).hexdigest()})
        (self.data / 'baseline.wmap').write_bytes(b'baseline')
        self.catalog.insert(0, {'kind': 'baseline', 'path': 'baseline.wmap',
                                'sha256': hashlib.sha256(b'baseline').hexdigest()})
        self.write_catalog()

    def write_catalog(self):
        (self.data / 'realm-catalog.json').write_text(json.dumps(self.catalog))

    def compile(self, command, check):
        self.assertTrue(check)
        realm_id = command[command.index('--id') + 1]
        data = Path(command[command.index('--data') + 1])
        output = data / 'realms' / f'{realm_id}.wmap'
        if '--check' in command:
            self.assertEqual(output.read_bytes(), self.outputs[realm_id])
        else:
            self.assertFalse(output.exists())
            if realm_id == 'one':
                self.assertFalse((data / 'realms/two.wmap').exists())
            output.write_bytes(self.outputs[realm_id])

    def test_rebuilds_and_checks_every_staged_recipe_without_changing_staged_data(self):
        before = {p: p.read_bytes() for p in self.data.rglob('*') if p.is_file()}
        with patch('realm_contract.subprocess.run', side_effect=self.compile) as run:
            check_realms('/unused/binary', self.data, self.root / 'copy')
        self.assertEqual(run.call_count, 4)
        self.assertEqual(before, {p: p.read_bytes() for p in self.data.rglob('*') if p.is_file()})

    def test_missing_and_modified_catalog_outputs_fail_before_compilation(self):
        for name in ['baseline.wmap', 'realms/one.wmap', 'realms/two.wmap']:
            path = self.data / name
            original = path.read_bytes()
            with self.subTest(name=name), patch('realm_contract.subprocess.run') as run:
                path.unlink()
                with self.assertRaises(FileNotFoundError):
                    check_realms('/unused/binary', self.data, self.root / f'missing-{path.stem}')
                path.write_bytes(b'corrupt')
                with self.assertRaisesRegex(RuntimeError, 'catalog hash mismatch'):
                    check_realms('/unused/binary', self.data, self.root / f'corrupt-{path.stem}')
                run.assert_not_called()
            path.write_bytes(original)

    def test_rebuilt_byte_mismatch_is_rejected(self):
        def different(command, check):
            self.compile(command, check)
            (self.root / 'copy/realms/one.wmap').write_bytes(b'different')
        with patch('realm_contract.subprocess.run', side_effect=different):
            with self.assertRaisesRegex(RuntimeError, 'differs from committed bytes: one'):
                check_realms('/unused/binary', self.data, self.root / 'copy')

    def test_compiler_failure_is_not_hidden(self):
        with patch('realm_contract.subprocess.run', side_effect=subprocess.CalledProcessError(1, 'compiler')):
            with self.assertRaises(subprocess.CalledProcessError):
                check_realms('/unused/binary', self.data, self.root / 'copy')

    def test_empty_reconstructions_and_unknown_kind_fail(self):
        for label, entries, message in [('empty', self.catalog[:1], 'no reconstruction'),
                                         ('unknown', [{'kind': 'unknown'}], 'Unknown Realm')]:
            self.catalog = entries
            self.write_catalog()
            with self.subTest(label=label), self.assertRaisesRegex(RuntimeError, message):
                check_realms('/unused/binary', self.data, self.root / label)
