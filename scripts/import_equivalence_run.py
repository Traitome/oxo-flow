#!/usr/bin/env python3
# Corpus runner for scripts/import-equivalence.sh.
import importlib.util, os, subprocess, sys, tomllib
ROOT, BIN, TMP = sys.argv[1], sys.argv[2], sys.argv[3]
HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location(
    'expect', os.path.join(HERE, 'import_equivalence_expectations.py'))
expect = importlib.util.module_from_spec(spec)
spec.loader.exec_module(expect)

FIX = os.path.join(ROOT, 'tests', 'fixtures', 'import')
cases = []
for sub in ('nextflow', 'snakemake'):
    d = os.path.join(FIX, sub)
    for name in sorted(os.listdir(d)):
        cases.append(sub + '/' + name)


def run_import(rel):
    src = os.path.join(FIX, rel)
    out = os.path.join(TMP, rel.replace('/', '_') + '.oxoflow')
    p = subprocess.run([BIN, 'import', src, '-o', out],
                       capture_output=True, text=True)
    return p.returncode, out, p.stderr


def tok_present(shell, tok):
    return ('{' + tok + '}') in shell or ('{' + tok + '[') in shell


def cmp_rule(name, spec, got, diffs):
    res = got.get('resources', {})
    if spec.get('threads') is not None and res.get('threads') != spec['threads']:
        diffs.append('%s.threads=%r != %r' % (name, res.get('threads'), spec['threads']))
    if spec.get('threads') is None and 'threads' in res:
        diffs.append('%s.threads added %r' % (name, res['threads']))
    if spec.get('memory') is not None and res.get('memory') != spec['memory']:
        diffs.append('%s.memory=%r != %r' % (name, res.get('memory'), spec['memory']))
    if spec.get('time_limit') is not None and res.get('time_limit') != spec['time_limit']:
        diffs.append('%s.time_limit=%r != %r' % (name, res.get('time_limit'), spec['time_limit']))
    if spec.get('environment') is not None and got.get('environment') != spec['environment']:
        diffs.append('%s.environment=%r != %r' % (name, got.get('environment'), spec['environment']))
    if spec.get('input') is not None and got.get('input') != spec['input']:
        diffs.append('%s.input=%r != %r' % (name, got.get('input'), spec['input']))
    if spec.get('output') is not None and got.get('output') != spec['output']:
        diffs.append('%s.output=%r != %r' % (name, got.get('output'), spec['output']))
    for k in ('localrule', 'temporary'):
        if spec.get(k) is not None and bool(got.get(k)) != spec[k]:
            diffs.append('%s.%s=%r != %r' % (name, k, got.get(k), spec[k]))
    if spec.get('temp_output') is not None and got.get('temp_output') != spec['temp_output']:
        diffs.append('%s.temp_output=%r != %r' % (name, got.get('temp_output'), spec['temp_output']))
    shell = got.get('shell', '')
    if not shell.startswith(spec['shell_verb']):
        diffs.append('%s.shell=%r not verb %r' % (name, shell[:40], spec['shell_verb']))
    for tok in spec.get('shell_tokens', []):
        if not tok_present(shell, tok):
            diffs.append('%s.shell missing {%s}' % (name, tok))


rows = []
for rel in cases:
    exp = expect.EXPECTED.get(rel)
    rejected = expect.REJECTED.get(rel)
    rc, out, err = run_import(rel)
    if rc != 0:
        if exp is not None:
            rows.append((rel, 'supported', 'REJECTED?', 1, 'exit=%d (expected import)' % rc))
        else:
            rows.append((rel, 'rejected', 'importer-rejected', 0, rejected or 'unspecified'))
        continue
    if exp is None:
        rows.append((rel, 'rejected', 'ACCEPTED?', 1, 'imported but expected a hard error'))
        continue
    with open(out, 'rb') as fh:
        wf = tomllib.load(fh)
    rules = wf.get('rules', [])
    got_names = [r['name'] for r in rules]
    by_name = {r['name']: r for r in rules}
    diffs = []
    if got_names != exp['rules']:
        diffs.append('rules=%r != %r' % (got_names, exp['rules']))
    # DAG edges: the engine derives them from output->input path matching,
    # plus any explicit depends_on. Union, deduped, in corpus order.
    produced = {}
    for r in rules:
        for p in r.get('output', []):
            produced.setdefault(p, []).append(r['name'])
    got_edges = []
    for r in rules:
        deps = list(r.get('depends_on', []))
        for p in r.get('input', []):
            deps += produced.get(p, [])
        for dep in deps:
            e = [dep, r['name']]
            if dep != r['name'] and e not in got_edges:
                got_edges.append(e)
    if got_edges != exp['edges']:
        diffs.append('edges=%r != %r' % (got_edges, exp['edges']))
    if 'wildcard_constraints' in exp:
        if wf.get('wildcard_constraints') != exp['wildcard_constraints']:
            diffs.append('wildcard_constraints=%r != %r'
                         % (wf.get('wildcard_constraints'), exp['wildcard_constraints']))
    for nm, spec in exp['rule_specs'].items():
        if nm not in by_name:
            diffs.append('rule %s missing' % nm)
            continue
        cmp_rule(nm, spec, by_name[nm], diffs)
    if diffs:
        rows.append((rel, 'supported', 'DRIFTED', 1, '; '.join(diffs)))
    else:
        rows.append((rel, 'supported', 'matched', 0, ''))

# report
print('%-46s %-10s %-16s %s' % ('case', 'arm', 'verdict', 'detail'))
print('-' * 100)
n = {'matched': 0, 'DRIFTED': 0, 'importer-rejected': 0, 'REJECTED?': 0, 'ACCEPTED?': 0}
for rel, arm, verdict, bad, detail in rows:
    n[verdict] = n.get(verdict, 0) + 1
    print('%-46s %-10s %-16s %s' % (rel, arm, verdict, detail[:60]))
print()
print('CONFUSION MATRIX (corpus=%d)' % len(rows))
print('  matched           : %d' % n.get('matched', 0))
print('  drifted           : %d' % n.get('DRIFTED', 0))
print('  importer-rejected : %d' % n.get('importer-rejected', 0))
print('  not-attempted     : 0')
fail = n.get('DRIFTED', 0) + n.get('REJECTED?', 0) + n.get('ACCEPTED?', 0)
if fail:
    print('DRIFT/UNEXPECTED: %d case(s)' % fail)
sys.exit(1 if fail else 0)
