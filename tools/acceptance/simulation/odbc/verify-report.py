"""Audit completed real product evidence; never synthesizes product PASS results."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re

BASE = Path(__file__).resolve().parent
ROOT = BASE.parents[3]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load(path):
    return json.loads(path.read_text(encoding='utf-8-sig'))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--diagnostics', type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    report_path = args.report.resolve(strict=True)
    report = load(report_path)
    run = report_path.parent
    listing = load(run / 'catalogue.json')
    assert listing['catalogue_only'] is True
    actual = [row['kind'] for row in listing['kinds']]
    assert len(actual) == len(set(actual))
    assert report['actual_catalogue_kinds'] == actual
    assert report['selected_kinds'] == actual
    assert report['actual_catalogue_odbc_entries'] == len(actual)
    assert report['coverage_complete'] is True
    assert [row['kind'] for row in report['rows']] == actual
    test_source = ROOT / 'src-tauri/tests/sim_odbc_catalogue.rs'
    source_text = test_source.read_text(encoding='utf-8')
    step_declaration = re.search(r'const STEP_NAMES: &\[&str\] = &\[(.*?)\];', source_text, re.S)
    sentinel_source = re.search(r'let sentinel=format!\("([^"]+)"', source_text)
    assert sentinel_source, 'Expected exact sentinel format'
    sentinel_prefix = json.loads('"' + sentinel_source.group(1) + '"').removesuffix('{}')
    assert '中文' in sentinel_prefix and "O'Reilly" in sentinel_prefix and chr(92) in sentinel_prefix
    assert step_declaration, 'Expected product-step declaration'
    expected_steps = re.findall(r'"([a-z0-9_]+)"', step_declaration.group(1))
    assert len(expected_steps) == 33 and len(set(expected_steps)) == 33
    assert digest(Path(report['test_exe'])) == report['test_exe_sha256']
    for item in report['runtime'].values():
        assert digest(Path(item['path'])) == item['sha256']
    profile_counts = Counter()
    total_steps = 0
    for row in report['rows']:
        kind = row['kind']
        assert row['status'] == 'SIMULATED_ODBC_CONTRACT_PASS', kind
        assert row['vendor_server_status'] == row['default_vendor_driver_route'] == 'UNVERIFIED', kind
        assert row['non_vendor_driver'] is True and row['fixture_file_removed'] is True, kind
        assert row['process']['exit_code'] == 0 and row['process']['exit_hex'] == '0x00000000', kind
        assert row['process']['timed_out'] is False, kind
        native_report = load(run / (kind + '.json'))
        assert native_report.get('checkpoint') is not True, kind
        assert native_report['rows'][0]['steps'] == row['steps'], kind
        assert [s['name'] for s in row['steps']] == expected_steps, kind
        assert [s['sequence'] for s in row['steps']] == list(range(1, 34)), kind
        assert all(s['status'] == 'PASS' and s['elapsed_ms'] >= 0 for s in row['steps']), kind
        evidence = {s['name']: s.get('evidence') for s in row['steps']}
        page = evidence['ordered_page_unicode_null']
        sentinel = page['sentinel']
        assert sentinel.startswith(sentinel_prefix) and page['total'] == 5, kind
        assert [r[0] for r in page['rows']] == ['1', '2'], kind
        assert page['rows'][1][1] == sentinel and all(r[-1] is None for r in page['rows']), kind
        mutation = evidence['page_after_mutation']
        assert mutation['total'] == len(mutation['rows']) == 4, kind
        assert [r[0] for r in mutation['rows']] == ['0', '1', '2', '3'], kind
        assert mutation['rows'][0][1] == 'mutated', kind
        assert evidence['empty_page_metadata'] == {'row_count': 0, 'column_count': 3}, kind
        assert 'Parser' in evidence['invalid_sql_error']['error'] and '42000' in evidence['invalid_sql_error']['error'], kind
        assert '42000' in evidence['dialect_sqlstate_42000']['error'], kind
        assert '28000' in evidence['authentication_sqlstate_28000']['error'], kind
        trace = (run / (kind + '.sql-trace.log')).read_text(encoding='utf-8')
        assert 'original=THIS IS NOT SQL\ttranslated=THIS IS NOT SQL' in trace, kind
        assert 'returned=-1' in trace and 'CREATE TABLE' in trace and 'UPDATE ' in trace and 'DELETE FROM' in trace and 'DROP TABLE' in trace, kind
        assert Path(row['process']['stdout']).is_file() and Path(row['process']['stderr']).is_file(), kind
        profile_counts[row['dialect_profile']] += 1
        total_steps += len(row['steps'])
    expected_counts = {'catalogue_odbc_entries': len(actual), 'completed': len(actual), 'simulated_pass': len(actual), 'fail': 0, 'vendor_real_pass': 0, 'actual_catalogue_odbc_entries': len(actual)}
    assert report['counts'] == expected_counts
    audit = {'audit_status': 'PASS', 'audit_scope': 'completed real product evidence consistency and current artifact hashes',
             'product_report': str(report_path), 'product_report_sha256': digest(report_path), 'counts': report['counts'],
             'coverage_complete': True, 'steps_per_kind': len(expected_steps), 'total_pass_steps': total_steps,
             'profile_counts': dict(profile_counts), 'native_exit_zero': len(actual), 'crash': 0, 'timeout': 0,
             'product_test_source_sha256': digest(test_source), 'vendor_real_pass': 0}
    if args.diagnostics:
        diagnostic_path = args.diagnostics.resolve(strict=True)
        diagnostic = load(diagnostic_path)
        assert diagnostic['debug_only'] is True and diagnostic['counts'] == {'completed': 64, 'pass': 64, 'fail': 0}
        assert len(diagnostic['rows']) == 64 and all(r['exit_code'] == 0 for r in diagnostic['rows'])
        assert diagnostic['runtime'] == report['runtime']
        audit['debug_diagnostic_matrix'] = {'path': str(diagnostic_path), 'sha256': digest(diagnostic_path), 'counts': diagnostic['counts']}
    output = args.output or run / 'verification.json'
    output.write_text(json.dumps(audit, ensure_ascii=False, indent=2), encoding='utf-8')
    print(json.dumps(audit, ensure_ascii=True, indent=2))
    print(str(output))


if __name__ == '__main__':
    main()
